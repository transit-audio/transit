//! ADR 0044: stored representations survive; effective row heights are bounded.
use serde_json::json;
use transit_model::{ChannelFormat, Track, TrackHeight, TrackId, TrackKind};

#[test]
fn payload_heights_bound_both_ends_without_rewriting_the_payload() {
    for height in [TrackHeight::Custom(0), TrackHeight::Fit(0)] {
        assert_eq!(height.points(), 16.0);
    }
    for height in [TrackHeight::Custom(u16::MAX), TrackHeight::Fit(u16::MAX)] {
        assert_eq!(height.points(), 440.0);
        assert_eq!(height.step(true), height);
        assert_eq!(height.step(false), TrackHeight::Jumbo);
    }
}

#[test]
fn numeric_heights_clamp_round_half_up_and_snap_to_presets() {
    use TrackHeight::*;
    for (points, expected) in [
        (-100.0, Micro),
        (0.0, Micro),
        (16.49, Micro),
        (16.5, Custom(17)),
        (45.5, Small),
        (46.49, Small),
        (56.49, Custom(56)),
        (56.5, Custom(57)),
        (439.5, Extreme),
        (1e30, Extreme),
    ] {
        assert_eq!(TrackHeight::from_points(points), expected, "{points}");
    }
    for height in TrackHeight::ALL {
        assert_eq!(TrackHeight::from_points(height.points()), height);
        assert_eq!(TrackHeight::from_id(&height.label().to_ascii_uppercase()), Some(height));
    }
    assert_eq!(Custom(46).step(true), Medium);
    assert_eq!(Fit(46).step(false), Mini);
    assert_eq!(Fit(0).step(false), Fit(0));
}

#[test]
fn every_variant_keeps_its_serde_spelling_and_payload() {
    use TrackHeight::*;
    let cases = [
        (Micro, json!("Micro")),
        (Mini, json!("Mini")),
        (Small, json!("Small")),
        (Medium, json!("Medium")),
        (Large, json!("Large")),
        (Jumbo, json!("Jumbo")),
        (Extreme, json!("Extreme")),
        (Custom(57), json!({"Custom":57})),
        (Custom(0), json!({"Custom":0})),
        (Fit(300), json!({"Fit":300})),
        (Fit(u16::MAX), json!({"Fit":65535})),
    ];
    for (height, value) in cases {
        assert_eq!(serde_json::to_value(height).unwrap(), value);
        assert_eq!(serde_json::from_value::<TrackHeight>(value).unwrap(), height);
    }
}

#[test]
fn a_new_track_uses_small() {
    for kind in TrackKind::ALL {
        let track = Track::new(TrackId(1), "New", kind, ChannelFormat::Stereo, [0; 3]);
        assert_eq!(track.height, TrackHeight::Small);
    }
}
