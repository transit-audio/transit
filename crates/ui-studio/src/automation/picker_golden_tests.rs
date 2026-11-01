//! Capture the default track's picker before adding MIDI effects and rack macros (#239).

#[test]
fn default_track_picker_entries_are_unchanged() {
    let (mut app, track) = super::app_with_track("audio");
    let entries = super::listed(&mut app, track);
    assert_eq!(
        entries,
        [("Mixer", "Volume", "volume"), ("Mixer", "Pan", "pan"), ("Mixer", "Mute", "mute")].map(|(section, label, id)| (
            section.to_string(),
            label.to_string(),
            id.to_string()
        ))
    );
}
