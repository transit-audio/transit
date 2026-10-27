//! More catalog → command mappings, for catalog entries served by one parameterised command
//! (e.g. every `Track > Change Track Width > <format>` runs `track.change_width` with that format).

/// Catalog path → command id.
pub const ALIASES_MORE: &[(&str, &str)] = &[
    ("Track > Change Track Width > Mono", "track.change_width"),
    ("Track > Change Track Width > Stereo", "track.change_width"),
    ("Track > Change Track Width > LCR", "track.change_width"),
    ("Track > Change Track Width > Quad", "track.change_width"),
    ("Track > Change Track Width > LCRS", "track.change_width"),
    ("Track > Change Track Width > 5.0", "track.change_width"),
    ("Track > Change Track Width > 5.1", "track.change_width"),
    ("Track > Change Track Width > 6.0", "track.change_width"),
    ("Track > Change Track Width > 6.1", "track.change_width"),
    ("Track > Change Track Width > 7.0 SDDS", "track.change_width"),
    ("Track > Change Track Width > 7.1 SDDS", "track.change_width"),
    ("Track > Change Track Width > 7.0", "track.change_width"),
    ("Track > Change Track Width > 7.1", "track.change_width"),
    ("Track > Change Track Width > 7.0.2", "track.change_width"),
    ("Track > Change Track Width > 7.1.2", "track.change_width"),
    ("Track > Change Track Width > 1st Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 2nd Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 3rd Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 4th Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 5th Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 6th Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 7th Order Ambisonics", "track.change_width"),
    ("Track > Change Track Width > 5.0.2", "track.change_width"),
    ("Track > Change Track Width > 5.1.2", "track.change_width"),
    ("Track > Change Track Width > 5.0.4", "track.change_width"),
    ("Track > Change Track Width > 5.1.4", "track.change_width"),
    ("Track > Change Track Width > 7.0.4", "track.change_width"),
    ("Track > Change Track Width > 7.1.4", "track.change_width"),
    ("Track > Change Track Width > 7.0.6", "track.change_width"),
    ("Track > Change Track Width > 7.1.6", "track.change_width"),
    ("Track > Change Track Width > 9.0.4", "track.change_width"),
    ("Track > Change Track Width > 9.1.4", "track.change_width"),
    ("Track > Change Track Width > 9.0.6", "track.change_width"),
    ("Track > Change Track Width > 9.1.6", "track.change_width"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_width_aliases_name_real_formats() {
        for (path, _) in ALIASES_MORE.iter().filter(|(_, id)| *id == "track.change_width") {
            let leaf = path.rsplit(" > ").next().unwrap_or("");
            assert!(transit_model::ChannelFormat::from_id(leaf).is_some(), "no channel format for `{leaf}`");
        }
    }
}
