use transit_mix::{Message, PluginEvent};
/// The plug-in's view of pedal `cc` on channel 0: (frame, down).
pub(super) fn pedal_edges(heard: &[(u64, PluginEvent)], pedal: u8) -> Vec<(u64, bool)> {
    heard
        .iter()
        .filter_map(|(f, e)| match e.message {
            Message::Control { cc, value } if cc == pedal => Some((*f, value >= 64)),
            _ => None,
        })
        .collect()
}
