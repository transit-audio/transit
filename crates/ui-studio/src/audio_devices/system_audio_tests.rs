//! The System Audio block of the I/O paths section (#268), driven like a user over the stand-in
//! devices: no loopback is opened (the stand-in run has no player).
use super::*;
use crate::audio_devices_system::{DEFAULT_OUTPUT, ELSEWHERE_NOTE, RIGHTS_NOTE};

fn labels_of(t: &T, key: &str) -> Vec<String> {
    t.layout()["items"].as_array().unwrap().iter().filter(|i| i["value"] == key).map(|i| i["label"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn the_switch_and_the_output_wait_for_apply_and_the_rights_note_is_shown() {
    let mut t = T::new();
    t.open_window();
    t.section("section_io");
    if !transit_playback::system_audio::offered() {
        // A build without WASAPI loopback says how the platform gets there instead.
        T::rect(&t.layout()["controls"], Some(ELSEWHERE_NOTE), "system_audio_note");
        return;
    }
    T::rect(&t.layout()["controls"], Some(RIGHTS_NOTE), "system_audio_rights");
    T::rect(&t.layout()["controls"], Some("Off."), "system_audio_status");
    t.click(mid(t.control_rect("system_audio")));
    assert!(t.app.audio_devices.draft.system_audio && !t.app.ui.audio.system_audio, "a draft choice until Apply");
    T::rect(&t.layout()["controls"], Some("Starts when you press Apply."), "system_audio_status");
    assert!(t.layout()["pending"].is_string(), "Apply says something is pending");
    t.click(mid(t.control_rect("system_audio_output")));
    assert_eq!(labels_of(&t, "system_audio_output"), [DEFAULT_OUTPUT, "Speakers (Realtek)", "Speakers (Focusrite USB Audio)"]);
    t.click(mid(T::rect(&t.layout()["items"], Some("Speakers (Focusrite USB Audio)"), "system_audio_output")));
    assert_eq!(t.app.audio_devices.draft.system_audio_output.as_deref(), Some("Speakers (Focusrite USB Audio)"));
    t.click(mid(t.control_rect("apply")));
    assert!(t.app.ui.audio.system_audio, "Apply keeps it");
    let reopened = t.reopened();
    let last = reopened.last().expect("Apply reopened the devices");
    assert!(last.system_audio && last.system_audio_output.as_deref() == Some("Speakers (Focusrite USB Audio)"), "{last:?}");
}

#[test]
fn an_audio_track_is_offered_system_audio_only_while_it_is_on() {
    let mut t = T::new();
    t.app.engine.execute("track.new", &json!({"name": "Browser", "format": "stereo"})).unwrap();
    let s = t.app.engine.session_arc();
    let track = s.tracks.last().unwrap();
    let has = |on: bool| {
        crate::mixer::input_choices_with(&s, track, 2, t.app.engine.io_map(), on)
            .into_iter()
            .any(|c| matches!(c, crate::mixer::RoutingChoice::Entry { value, .. } if value == "System Audio"))
    };
    assert!(has(true) && !has(false));
    // Picking it routes the track to the built-in input, never a new bus named after it.
    t.app.engine.execute("track.input", &json!({"tracks": [track.id.0], "input": "system audio"})).unwrap();
    let s = t.app.engine.session();
    assert_eq!(s.tracks.last().unwrap().mixer.input, transit_model::Route::Hardware("System Audio".into()));
    assert!(s.busses.is_empty(), "no bus was made for it");
}
