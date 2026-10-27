use transit_mix::{Message, PluginEvent};
/// Checks that every voice the instrument heard start was released after it, and returns the
/// number of note-ons.
pub(super) fn every_on_has_its_off(heard: &[(u64, PluginEvent)]) -> usize {
    let mut open = std::collections::HashMap::new();
    let mut ons = 0;
    for (f, e) in heard {
        match e.message {
            Message::NoteOn { .. } => {
                assert!(open.insert(e.note_id, *f).is_none(), "voice id {} reused while it sounds", e.note_id);
                ons += 1;
            }
            Message::NoteOff { .. } => {
                let start = open.remove(&e.note_id).unwrap_or_else(|| panic!("off for voice {} that is not sounding", e.note_id));
                assert!(*f >= start, "voice {} released at {f} before its on at {start}", e.note_id);
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "voices never released: {open:?}");
    ons
}
