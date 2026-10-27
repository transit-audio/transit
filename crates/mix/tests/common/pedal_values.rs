use transit_mix::{Message, PluginEvent};
/// The plug-in's view of pedal `cc`: (frame, value).
pub(super) fn pedal_values(heard: &[(u64, PluginEvent)], pedal: u8) -> Vec<(u64, u8)> {
    heard
        .iter()
        .filter_map(|(f, e)| match e.message {
            Message::Control { cc, value } if cc == pedal => Some((*f, value)),
            _ => None,
        })
        .collect()
}
